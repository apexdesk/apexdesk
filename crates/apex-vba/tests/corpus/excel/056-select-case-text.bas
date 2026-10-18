' corpus-source: generated
' corpus-flagged: no
' corpus-prints: weekend
' corpus-prints: weekday
Function Kind(d As String) As String
    Select Case LCase(d)
        Case "sat", "sun"
            Kind = "weekend"
        Case Else
            Kind = "weekday"
    End Select
End Function
Sub Main()
    Debug.Print Kind("Sun")
    Debug.Print Kind("Tue")
End Sub
