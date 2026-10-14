' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Ann Lee
' corpus-prints: 28
'--- module Person class
Public FirstName As String
Public LastName As String
Private mAge As Long
Public Property Let Age(v As Long)
    mAge = v
End Property
Public Property Get Age() As Long
    Age = mAge
End Property
Public Function FullName() As String
    FullName = FirstName & " " & LastName
End Function
'--- module Module1 standard
Sub Main()
    Dim p As Person
    Set p = New Person
    p.FirstName = "Ann"
    p.LastName = "Lee"
    p.Age = 28
    Debug.Print p.FullName
    Debug.Print p.Age
End Sub
